//! The marker line that opens every Linewright comment (`docs/specs/product.md`,
//! "Visibility") and every issue a workflow files ("Workflows"), so people
//! and the manager can tell its writing from others'.

use std::fmt;

use crate::domain::{Comment, Login};
use crate::workflow::WorkflowName;

/// The stage of the pipeline a comment reports on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Triage,
    Tickets,
    Implementation,
    Ci,
    Review,
    Rework,
    Update,
}

impl Stage {
    const ALL: [Stage; 7] = [
        Stage::Triage,
        Stage::Tickets,
        Stage::Implementation,
        Stage::Ci,
        Stage::Review,
        Stage::Rework,
        Stage::Update,
    ];

    fn name(self) -> &'static str {
        match self {
            Stage::Triage => "triage",
            Stage::Tickets => "tickets",
            Stage::Implementation => "implementation",
            Stage::Ci => "CI",
            Stage::Review => "review",
            Stage::Rework => "rework",
            Stage::Update => "update",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A comment body opening with the stage's marker line.
pub fn marked(stage: Stage, body: &str) -> String {
    with_marker(stage, body)
}

/// The body of an issue the workflow `name` files, opening with its marker
/// line.
pub fn workflow_marked(name: &WorkflowName, body: &str) -> String {
    with_marker(format_args!("workflow {name}"), body)
}

fn with_marker(source: impl fmt::Display, body: &str) -> String {
    format!("{}\n\n{}", marker_line(source), body.trim())
}

/// The marker line of what Linewright wrote, such as a stage.
fn marker_line(source: impl fmt::Display) -> String {
    format!("> *Linewright, {source}*")
}

/// The stage of a comment the machine user wrote with a marker line, if it
/// is one.
pub fn stage_of(comment: &Comment, machine_user: &Login) -> Option<Stage> {
    if &comment.author != machine_user {
        return None;
    }
    let first = comment.body.lines().next()?.trim();
    Stage::ALL
        .into_iter()
        .find(|stage| first == marker_line(stage))
}
