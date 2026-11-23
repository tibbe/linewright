//! Every action a person can do, defined once with the access level it
//! needs (`docs/specs/access.md`, Actions;
//! `docs/specs/architecture.md`, Actions), and the reads its screens are
//! made of. Each takes a [`Permit`] of its level, and each that changes
//! something logs who did it, what it changed and how it ended.

mod authorizations;
mod automations;
mod benchmark_run;
mod benchmark_screens;
mod benchmarks;
mod board;
mod capture;
mod item;
mod people;
mod run;
mod work;

pub use authorizations::*;
pub use automations::*;
pub use benchmark_run::*;
pub use benchmark_screens::*;
pub use benchmarks::*;
pub use board::*;
pub use capture::*;
pub use item::*;
pub use people::*;
pub use run::*;
pub use work::*;

use std::fmt::Display;

use tracing::{info, warn};

use crate::access::{Caller, LevelMarker, NeedsLevel, Permit};
use crate::domain::Number;

/// An action of `docs/specs/access.md`'s table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    GetBoard,
    GetItem,
    ListRuns,
    GetRun,
    ListAutomations,
    ListBenchmarks,
    GetBenchmark,
    GetTask,
    GetBenchmarkRun,
    GetServer,
    ListPeople,
    ListAuthorizations,
    EndAuthorization,
    AddTask,
    CreateBenchmark,
    EditBenchmark,
    EditTask,
    RemoveTask,
    RunBenchmark,
    EditConfiguration,
    CancelBenchmarkRun,
    EditPerson,
    DisablePerson,
    EnablePerson,
}

impl Action {
    pub const ALL: [Action; 24] = [
        Action::GetBoard,
        Action::GetItem,
        Action::ListRuns,
        Action::GetRun,
        Action::ListAutomations,
        Action::ListBenchmarks,
        Action::GetBenchmark,
        Action::GetTask,
        Action::GetBenchmarkRun,
        Action::GetServer,
        Action::ListPeople,
        Action::ListAuthorizations,
        Action::EndAuthorization,
        Action::AddTask,
        Action::CreateBenchmark,
        Action::EditBenchmark,
        Action::EditTask,
        Action::RemoveTask,
        Action::RunBenchmark,
        Action::EditConfiguration,
        Action::CancelBenchmarkRun,
        Action::EditPerson,
        Action::DisablePerson,
        Action::EnablePerson,
    ];

    /// The access level it needs.
    pub fn level(self) -> crate::access::Level {
        use crate::access::Level;
        match self {
            Action::GetBoard
            | Action::GetItem
            | Action::ListRuns
            | Action::GetRun
            | Action::ListAutomations
            | Action::ListBenchmarks
            | Action::GetBenchmark
            | Action::GetTask
            | Action::GetBenchmarkRun
            | Action::GetServer
            | Action::ListPeople
            | Action::ListAuthorizations
            | Action::EndAuthorization => Level::Read,
            Action::AddTask
            | Action::CreateBenchmark
            | Action::EditBenchmark
            | Action::EditTask
            | Action::RemoveTask
            | Action::RunBenchmark
            | Action::EditConfiguration
            | Action::CancelBenchmarkRun => Level::Act,
            Action::EditPerson | Action::DisablePerson | Action::EnablePerson => Level::Manage,
        }
    }

    /// The name of its tool on the MCP server.
    pub fn tool_name(self) -> &'static str {
        match self {
            Action::GetBoard => "get_board",
            Action::GetItem => "get_item",
            Action::ListRuns => "list_runs",
            Action::GetRun => "get_run",
            Action::ListAutomations => "list_automations",
            Action::ListBenchmarks => "list_benchmarks",
            Action::GetBenchmark => "get_benchmark",
            Action::GetTask => "get_task",
            Action::GetBenchmarkRun => "get_benchmark_run",
            Action::GetServer => "get_server",
            Action::ListPeople => "list_people",
            Action::ListAuthorizations => "list_authorizations",
            Action::EndAuthorization => "end_authorization",
            Action::AddTask => "add_task",
            Action::CreateBenchmark => "create_benchmark",
            Action::EditBenchmark => "edit_benchmark",
            Action::EditTask => "edit_task",
            Action::RemoveTask => "remove_task",
            Action::RunBenchmark => "run_benchmark",
            Action::EditConfiguration => "edit_configuration",
            Action::CancelBenchmarkRun => "cancel_benchmark_run",
            Action::EditPerson => "edit_person",
            Action::DisablePerson => "disable_person",
            Action::EnablePerson => "enable_person",
        }
    }

    /// The action, as a title, such as of its tool
    /// (`docs/specs/access.md`, The MCP server, Tools).
    pub fn title(self) -> &'static str {
        match self {
            Action::GetBoard => "See the board",
            Action::GetItem => "See an item",
            Action::ListRuns => "See the runs",
            Action::GetRun => "See a run",
            Action::ListAutomations => "See the automations",
            Action::ListBenchmarks => "See the benchmarks",
            Action::GetBenchmark => "See a benchmark",
            Action::GetTask => "See a task",
            Action::GetBenchmarkRun => "See a benchmark run",
            Action::GetServer => "See Linewright itself",
            Action::ListPeople => "See the people",
            Action::ListAuthorizations => "See one's own authorizations",
            Action::EndAuthorization => "End one of one's own authorizations",
            Action::AddTask => "Capture a run as a task",
            Action::CreateBenchmark => "Make a benchmark",
            Action::EditBenchmark => "Change a benchmark",
            Action::EditTask => "Change a task",
            Action::RemoveTask => "Remove a task",
            Action::RunBenchmark => "Launch a benchmark run",
            Action::EditConfiguration => "Change a configuration",
            Action::CancelBenchmarkRun => "Cancel a benchmark run",
            Action::EditPerson => "Change a person's role",
            Action::DisablePerson => "Disable a person",
            Action::EnablePerson => "Enable a person",
        }
    }

    /// Whether it only shows something, changing nothing.
    pub fn read_only(self) -> bool {
        match self {
            Action::GetBoard
            | Action::GetItem
            | Action::ListRuns
            | Action::GetRun
            | Action::ListAutomations
            | Action::ListBenchmarks
            | Action::GetBenchmark
            | Action::GetTask
            | Action::GetBenchmarkRun
            | Action::GetServer
            | Action::ListPeople
            | Action::ListAuthorizations => true,
            Action::EndAuthorization
            | Action::AddTask
            | Action::CreateBenchmark
            | Action::EditBenchmark
            | Action::EditTask
            | Action::RemoveTask
            | Action::RunBenchmark
            | Action::EditConfiguration
            | Action::CancelBenchmarkRun
            | Action::EditPerson
            | Action::DisablePerson
            | Action::EnablePerson => false,
        }
    }

    /// `caller`'s permit of the level `L` for it, asked to change
    /// `changing`, or else the refusal, logged where it would have changed
    /// something.
    ///
    /// # Panics
    ///
    /// When `L` is not its level.
    pub fn permit<L: LevelMarker>(
        self,
        caller: &Caller,
        changing: &str,
    ) -> Result<Permit<L>, NeedsLevel> {
        assert_eq!(L::LEVEL, self.level(), "{self:?} needs its own level");
        caller.permit::<L>().inspect_err(|refused| {
            if !self.read_only() {
                logged(caller, self, changing, Ended::Refused(refused));
            }
        })
    }
}

/// How an action that changes something ended (`docs/specs/access.md`,
/// Actions, Logged).
#[derive(Clone, Copy)]
enum Ended<'a> {
    Done,
    Refused(&'a dyn Display),
    Failed(&'a dyn Display),
}

impl<'a> Ended<'a> {
    /// How an action that gave `result` ended.
    fn of<T, E: Ending>(result: &'a Result<T, E>) -> Ended<'a> {
        match result {
            Ok(_) => Ended::Done,
            Err(e) => match e.failure() {
                Some(failure) => Ended::Failed(failure),
                None => Ended::Refused(e),
            },
        }
    }
}

/// An error an action that changes something ends with: a refusal, or a
/// failure.
trait Ending: Display {
    /// What failed, `None` for a refusal.
    fn failure(&self) -> Option<&anyhow::Error>;
}

/// Logs that `caller` did `action`, which changed or would have changed
/// `changed`, and how it ended: a failure with all its causes.
fn logged(caller: &Caller, action: Action, changed: &str, ended: Ended) {
    let person = caller.person();
    let (id, identity) = (person.id, person.logged());
    let (tool, title) = (action.tool_name(), action.title());
    match ended {
        Ended::Done => {
            info!(person = %id, identity, action = tool, changed, "{title}: done");
        }
        Ended::Refused(why) => {
            info!(person = %id, identity, action = tool, changed, reason = %why, "{title}: refused");
        }
        Ended::Failed(why) => {
            let error = format!("{why:#}");
            warn!(person = %id, identity, action = tool, changed, error, "{title}: failed");
        }
    }
}

/// The refusal of a required field, or argument, that is empty or holds
/// only white space (`docs/specs/ui_patterns.md`, Making, Required).
pub const REQUIRED: &str = "Required.";

/// What an action or a link named that does not exist, and the refusal
/// that names it (`docs/specs/ui_patterns.md`, Screen states, Nothing
/// there). A thing is named as it was given, whether or not it parses.
#[derive(Clone, Debug)]
pub enum Missing {
    Item(Number),
    Run(String),
    Benchmark(String),
    Task(String),
    BenchmarkRun(String),
    /// A configuration of a benchmark run, which has no screen of its own.
    Configuration(String),
    /// A person, who has no screen of their own.
    Person(String),
    /// One of the reader's own authorizations.
    Authorization(String),
}

impl Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Missing::Item(number) => write!(f, "No such item {number}"),
            Missing::Run(id) => write!(f, "No such run {id}"),
            Missing::Benchmark(id) => write!(f, "No such benchmark {id}"),
            Missing::Task(id) => write!(f, "No such task {id}"),
            Missing::BenchmarkRun(id) => write!(f, "No such benchmark run {id}"),
            Missing::Configuration(id) => write!(f, "No such configuration {id}"),
            Missing::Person(id) => write!(f, "No such person {id}"),
            Missing::Authorization(id) => write!(f, "No such authorization {id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows of the table of `docs/specs/access.md`, Actions: each
    /// action, its access level and its tool.
    fn spec_actions() -> Vec<(String, String, String)> {
        let spec = include_str!("../../../docs/specs/access.md");
        let section = spec
            .split("\n## Actions\n")
            .nth(1)
            .expect("the spec has Actions");
        section
            .lines()
            .skip_while(|line| !line.starts_with('|'))
            .take_while(|line| line.starts_with('|'))
            .skip(2)
            .map(|row| {
                let cells: Vec<&str> = row.split('|').map(str::trim).collect();
                (
                    cells[1].to_string(),
                    cells[2].to_string(),
                    cells[4].trim_matches('`').to_string(),
                )
            })
            .collect()
    }

    /// `docs/specs/access.md`, Actions.
    #[test_log::test]
    fn each_action_has_the_level_and_tool_of_the_specs_table() {
        let defined: Vec<(String, String, String)> = Action::ALL
            .iter()
            .map(|action| {
                let mut title = action.title().chars();
                let first = title.next().unwrap().to_lowercase();
                (
                    first.chain(title).collect(),
                    action.level().to_string(),
                    action.tool_name().to_string(),
                )
            })
            .collect();

        assert_eq!(defined, spec_actions());
    }

    /// `docs/specs/access.md`, The MCP server, Tools: a tool that only shows
    /// something is marked read-only.
    #[test_log::test]
    fn the_actions_that_only_show_something_are_read_only() {
        let read_only: Vec<&str> = Action::ALL
            .iter()
            .filter(|action| action.read_only())
            .map(|action| action.tool_name())
            .collect();

        assert_eq!(
            read_only,
            [
                "get_board",
                "get_item",
                "list_runs",
                "get_run",
                "list_automations",
                "list_benchmarks",
                "get_benchmark",
                "get_task",
                "get_benchmark_run",
                "get_server",
                "list_people",
                "list_authorizations",
            ]
        );
    }
}
