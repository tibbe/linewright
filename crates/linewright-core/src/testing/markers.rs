use std::sync::Arc;

use crate::briefs::Briefs;
use crate::prompts::OwnTemplates;

/// Linewright's own layer with each file a marker: its path and each of its
/// placeholders, as `marker` writes them, on one line.
#[track_caller]
pub fn markers() -> OwnTemplates {
    OwnTemplates::generate(marker_template).expect("every marker has its file's placeholders")
}

/// The text of the file at `path` under `briefs/` in `markers`, for a test
/// to put in a layer of its own.
///
/// # Panics
///
/// When no file of the brief Linewright fills is at `path`, or a marker
/// lacks its file's placeholders.
#[track_caller]
pub fn marker_text(path: &str) -> String {
    let mut found = None;
    OwnTemplates::generate(|at, names| {
        let text = marker_template(at, names);
        if at == path {
            found = Some(text.clone());
        }
        text
    })
    .expect("every marker has its file's placeholders");
    found.unwrap_or_else(|| panic!("no file of the brief is at {path}"))
}

/// The marker of the file at `path` with a placeholder for each of `names`,
/// and the newline a file ends with.
fn marker_template(path: &str, names: &[&str]) -> String {
    let placeholders: Vec<(&str, String)> = names
        .iter()
        .map(|name| (*name, format!("{{{{{name}}}}}")))
        .collect();
    let values: Vec<(&str, &str)> = placeholders
        .iter()
        .map(|(name, placeholder)| (*name, placeholder.as_str()))
        .collect();
    format!("{}\n", marker(path, &values))
}

/// The briefs with no layer over the markers.
#[track_caller]
pub fn marker_briefs() -> Arc<Briefs> {
    Arc::new(Briefs::resolve(&markers(), &[]).briefs)
}

/// The marker of the file at `path` under `briefs/` filled with `values`,
/// each by its name, in the code's order: `[path name=«value» …]`, with no
/// newline after it.
pub fn marker(path: &str, values: &[(&str, &str)]) -> String {
    let mut marker = format!("[{path}");
    for (name, value) in values {
        marker.push_str(&format!(" {name}=«{value}»"));
    }
    marker.push(']');
    marker
}

/// The value of the first placeholder `name` filled in a marker in
/// `marked`, whole where it holds markers of its own.
///
/// # Panics
///
/// When `marked` fills no placeholder `name`, or does not end its value.
#[track_caller]
pub fn filled_in<'a>(marked: &'a str, name: &str) -> &'a str {
    let opening = format!(" {name}=«");
    let start = marked
        .find(&opening)
        .unwrap_or_else(|| panic!("no marker fills {name}:\n{marked}"))
        + opening.len();
    let mut depth = 0;
    for (at, c) in marked[start..].char_indices() {
        match c {
            '«' => depth += 1,
            '»' if depth == 0 => return &marked[start..start + at],
            '»' => depth -= 1,
            _ => {}
        }
    }
    panic!("the value of {name} has no end:\n{marked}")
}

/// What a session is sent back with when its result can't be used for
/// `reason`.
pub fn could_not_be_used(reason: &str) -> String {
    marker("prompts/follow-ups/result-error.md", &[("error", reason)])
}

/// What a session is sent back with when its work is not finished, for
/// `problem`.
pub fn not_finished(problem: &str) -> String {
    marker("prompts/follow-ups/loose-ends.md", &[("problem", problem)])
}
