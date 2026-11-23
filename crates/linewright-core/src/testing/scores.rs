use std::path::Path;

use serde_json::{Value, json};

use super::trial_runs::entries;

/// A scorer's result: the verdict `verdict`, and a leak found when `calls`
/// quotes any.
pub fn scored_as(verdict: &str, calls: &[&str]) -> String {
    json!({
        "outcome": "judged",
        "correctness": {"reasoning": format!("It is a {verdict}."), "verdict": verdict},
        "leak": {
            "reasoning": "I read every transcript.",
            "found": !calls.is_empty(),
            "calls": calls,
        },
    })
    .to_string()
}

/// The score kept in the output `out`.
#[track_caller]
pub fn score_in(out: &Path) -> Value {
    let text = std::fs::read_to_string(out.join("score.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The names in the scorers' directory of the output `out`.
#[track_caller]
pub fn scorer_records(out: &Path) -> Vec<String> {
    let dir = out.join("scorer");
    if !dir.exists() {
        return vec![];
    }
    entries(&dir)
}
