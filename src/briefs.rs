//! The briefs: one Markdown file per stage, read at startup from the `briefs`
//! setting so a change needs no rebuild (`docs/specs/product.md`, "Replay"), and one
//! for workflow runs. The ones Linewright ships live in `briefs/` and are
//! built in.

use std::path::Path;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Briefs {
    pub triage: String,
    pub to_tickets: String,
    pub implement: String,
    pub tdd: String,
    pub code_review: String,
    pub pr: String,
    pub workflow: String,
}

const FILES: [&str; 7] = [
    "triage.md",
    "to-tickets.md",
    "implement.md",
    "tdd.md",
    "code-review.md",
    "pr.md",
    "workflow.md",
];

impl Briefs {
    pub fn shipped() -> Briefs {
        Briefs {
            triage: include_str!("../briefs/triage.md").into(),
            to_tickets: include_str!("../briefs/to-tickets.md").into(),
            implement: include_str!("../briefs/implement.md").into(),
            tdd: include_str!("../briefs/tdd.md").into(),
            code_review: include_str!("../briefs/code-review.md").into(),
            pr: include_str!("../briefs/pr.md").into(),
            workflow: include_str!("../briefs/workflow.md").into(),
        }
    }

    /// Reads every brief from `dir`, failing on the first one missing.
    pub fn load(dir: &Path) -> Result<Briefs> {
        let read = |name: &str| {
            let path = dir.join(name);
            std::fs::read_to_string(&path)
                .with_context(|| format!("reading the brief {}", path.display()))
        };
        let [
            triage,
            to_tickets,
            implement,
            tdd,
            code_review,
            pr,
            workflow,
        ] = FILES.map(read);
        Ok(Briefs {
            triage: triage?,
            to_tickets: to_tickets?,
            implement: implement?,
            tdd: tdd?,
            code_review: code_review?,
            pr: pr?,
            workflow: workflow?,
        })
    }
}
